---
id: TASK-111.7
title: >-
  Version stamp: extentd, fxdaxctl, the Firecracker extent device and the guest
  module report their build commit
status: To Do
assignee: []
created_date: '2026-10-28 01:25'
labels: []
dependencies: []
parent_task_id: TASK-111
priority: medium
ordinal: 421000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Every sandbox result should trace to an exact build (TASK-111); today they record commits by hand.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 extentd /metrics exports its build commit
- [ ] #2 fxdaxctl --version prints its build commit
- [ ] #3 The VMM reports its build commit to extentd and /metrics shows it per VM
- [ ] #4 The guest module shows its build commit under /proc/fs/fxfs
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
