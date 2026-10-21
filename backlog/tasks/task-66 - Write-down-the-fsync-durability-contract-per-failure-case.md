---
id: TASK-66
title: Write down the fsync durability contract per failure case
status: To Do
assignee: []
created_date: '2026-10-21 19:54'
labels: []
dependencies: []
priority: medium
ordinal: 175000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A guest fsync is an ordered COMMIT, durable within 50 ms and uploaded within about 1 s (doc-2). Write down what an acknowledged fsync survives: the VM dying, extentd dying, a host reboot, host loss.
<!-- SECTION:DESCRIPTION:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
