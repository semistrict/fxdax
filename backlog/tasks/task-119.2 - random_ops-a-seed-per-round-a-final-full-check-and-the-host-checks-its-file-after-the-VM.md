---
id: TASK-119.2
title: >-
  random_ops: a seed per round, a final full check, and the host checks its file
  after the VM
status: To Do
assignee: []
created_date: '2026-10-28 19:27'
labels:
  - testing
dependencies: []
parent_task_id: TASK-119
priority: high
ordinal: 438000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
random_ops always runs seed 1, has no full check after its last op, compares only 1 MiB + 4 KiB of a file reaching 1 MiB + 16 KiB, and removes its file, so the host never reads what the store holds.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Each round uses a different seed, printed, replayable by setting it
- [ ] #2 The whole file is checked after the last op
- [ ] #3 The host compares the stored file with the model after the VM exits
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
