---
id: TASK-160.14
title: 'extent_remote: a branch record''s six flags as one decoded state'
status: To Do
assignee: []
created_date: '2026-11-03 13:08'
labels:
  - structure
dependencies: []
parent_task_id: TASK-160
priority: high
type: task
ordinal: 585000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Record's origin/forked/deleted/point/forking/claimed flags act as an unwritten state machine: built by hand at 7 sites, interpreted in four modules, each create-then-look site with its own whose-record test, the Lost-is-fine rule written twice, five read-modify-write loops (take_lease and end_lease near-identical). Suspected, unconfirmed: record_fork accepts any record of an equal origin, so a direct fork and a fork of a point at one segment, given one guest-chosen ID, may take each other's. Explains TASK-79.82 (9+ fixes), 79.78, 79.84, 158. Proposal: enum Kind decoded once, refusing illegal combinations, and one branch::transition CAS loop for every create and update. The byte layout with TASK-153.9.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Records decoded once; every create and update through one transition
- [ ] #2 A table test over all 64 flag combinations
- [ ] #3 Sim profiles pass, each rule still switchable
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
