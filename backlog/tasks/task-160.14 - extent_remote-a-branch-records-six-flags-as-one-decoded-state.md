---
id: TASK-160.14
title: 'extent_remote: a branch record''s six flags as one decoded state'
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-03 13:08'
updated_date: '2026-11-03 14:23'
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

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Suspected bug: unit test, a direct fork and a fork of a point at one segment, one child ID, two owners; red with records_owned off (what the format bump's owner fixed), green on main. Plus a class test: every pair of record makers of one ID, of two owners, takes nothing of the other's.
2. record.rs: Record { format, owner, kind: Kind, deleted }, Kind { Root, Own, Fork(Origin), Point(Origin), Forking { origin, point }, Claim(Origin) }, decoded once from the wire's flags (unchanged bytes) through serde try_from, refusing illegal combinations; queries writable, is_child_of, may_drop, origin; branch::lost_is_ok for the Lost-is-fine rule.
3. branch::transition: one CAS loop (get, step, create or conditional update, again) that every record and lease create and update goes through: create, create_forked, record_fork, fork_point, finish_point_fork, abandon, delete, mark_deleted, delete_fork, take_lease, end_lease.
4. ForkedAt goes: delete_fork takes the Kind its child's record is or would be; fxdax's Claim holds a Kind once its segment goes.
5. Table test over all 64 flag combinations. Sim rules stay inside the transitions.
6. Proof: all test_needs!/test_holds! profiles, SIM_SOAK of restores and forks_and_frees, scripts/test.sh, GCE test-gce.sh all with guest snapshot/migrate/fork cases.
<!-- SECTION:PLAN:END -->
