---
id: TASK-39.7
title: >-
  Fencing on takeover with the placement's boot ID, and a call that waits for
  uploads
status: To Do
assignee: []
created_date: '2026-10-17 19:16'
labels: []
dependencies: []
parent_task_id: TASK-39
priority: high
type: feature
ordinal: 117000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
When a sandbox moves to a new host, the store must accept the placement's boot ID as its fencing token, so that a cut-off host cannot overwrite the new owner's data. The uploader takes the branch's lease as a random holder (crates/fxdax/src/remote.rs, extentd open): the token should be the boot ID, and a later boot ID must fence an earlier one. And before a clean stop, the system needs a call that waits until a store's data is in object storage (today only inside a durable COMMIT).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A store opens with a fencing token (the placement's boot ID); a writer with a later token fences one with an earlier, whose commits then fail and whose uploads cannot land, shown by tests (extent_remote simulation and fxdax)
- [ ] #2 extentd and fxdaxctl expose a call that returns once every commit of a store is uploaded, shown by a test
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
