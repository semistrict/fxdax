---
id: TASK-131.2
title: >-
  fsync without a seal: commit without write-protecting and evacuating, sealing
  only at fork, snapshot or reclaim
status: To Do
assignee: []
created_date: '2026-10-29 21:13'
labels:
  - performance
dependencies: []
parent_task_id: TASK-131
priority: high
ordinal: 473000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Sandbox team round 8: DAX seals cost 0.54 ms against 0.32 without DAX, almost all ~3 window remaps; if fsync could commit without write-protecting and evacuating, and seal only at fork, snapshot or reclaim, DAX writes could match no-DAX. TASK-112.3 (doc-9) answered round 7b: every fsync must seal, write-once extents underpinning durability by whole extents, sharing by ID and the shared page cache; a durable-but-open staging extent costs store-format, protection and seal-on-share changes. Revisit with their numbers: which of the seal's remaps are on the commit path now, and what a durable open extent would take.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A design answer with this round's numbers, and the remaps left on the commit path measured
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
