---
id: TASK-164.1
title: 'Host: copy-on-write DAX window pages'
status: To Do
assignee: []
created_date: '2026-11-04 07:58'
labels: []
dependencies: []
parent_task_id: TASK-164
priority: high
type: task
ordinal: 611000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
DAX_COW, private_max, MAP_ZEROS, PRIVATE in the extent device (doc-13 stage 1), offered beside today's writable mode.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A store to a sealed or zero page gets a private copy at the same window address; MAP and UNMAP free it; eviction skips it
- [ ] #2 Past private_max refused (exit 159); KVM's async writes don't copy
- [ ] #3 doc-4 says so; unit, Shuttle and GCE tests pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
