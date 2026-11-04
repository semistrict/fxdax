---
id: TASK-164.7
title: Delete writable DAX
status: To Do
assignee: []
created_date: '2026-11-04 07:58'
labels: []
dependencies: []
parent_task_id: TASK-164
priority: high
type: task
ordinal: 617000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
doc-13 stage 4: guest, Fxfs, host, protocol, specs, sim, patches 0005/0008/0009.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Every list in doc-13's What goes done
- [ ] #2 cargo nextest run --workspace on macOS and GCE; scripts/upstream-changes.py --check passes
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
