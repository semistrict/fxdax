---
id: TASK-144
title: 'readdir''s d_type of a device, fifo or socket'
status: To Do
assignee: []
created_date: '2026-10-31 22:07'
labels: []
dependencies: []
priority: low
ordinal: 494000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
generic/401: Fxfs's ObjectDescriptor has File for special nodes too, so readdir says DT_REG, and find -type p misses them. A lookup per entry would slow every readdir; a new descriptor touches the format and the matches on it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 d_type is right for every node, without a per-entry lookup, generic/401 passes
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
