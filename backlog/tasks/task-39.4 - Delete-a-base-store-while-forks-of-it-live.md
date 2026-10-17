---
id: TASK-39.4
title: Delete a base store while forks of it live
status: To Do
assignee: []
created_date: '2026-10-17 19:16'
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
