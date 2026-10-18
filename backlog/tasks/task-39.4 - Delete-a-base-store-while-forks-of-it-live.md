---
id: TASK-39.4
title: Delete a base store while forks of it live
status: In Progress
assignee: []
created_date: '2026-10-17 19:16'
updated_date: '2026-10-18 00:29'
labels: []
dependencies: []
parent_task_id: TASK-39
priority: high
type: feature
ordinal: 114000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A base store cannot be deleted while forks of it exist (extent_store::delete_host: 'forks of the store live'; a lineage's first branch is the store). Snapshot deletion needs to delete a base and keep its forks, whose shared data stays until no branch holds it, locally and in object storage.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Deleting a base store with live forks succeeds: its own data goes, what its forks share stays, and the forks read and write as before, shown by extent_store and fxdaxctl tests
- [ ] #2 The same holds for branches in object storage: the collector keeps what live forks need of a deleted base, shown by the simulation
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. extent_store: a store's first branch can be deleted while forks live: its record stays as a tombstone (the store's directory then holds its forks' data alone, and opening the store fails, saying so); delete_host deletes it, and removes the directory once its last branch goes, whichever is last.
2. extentd DELETE_PATH, fxdaxctl delete through FXDAX_EXTENTD: deletion runs in the process that has the lineage open (the daemon serving the forks), refusing a store a connection has open (EBUSY); with a bucket, the branch is deleted there too (branch::delete), its forks' records made first.
3. Simulation profile: bases deleted while their forks live and write, with collectors running; the collector keeps what forks need.
4. Tests: extent_store (base deleted with live forks: own data goes, shared stays, forks read/write/fork, last fork deletes the directory), fxdaxctl/extentd test, simulation.
<!-- SECTION:PLAN:END -->
