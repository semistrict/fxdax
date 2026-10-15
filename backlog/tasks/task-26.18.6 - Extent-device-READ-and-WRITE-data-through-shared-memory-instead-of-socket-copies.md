---
id: TASK-26.18.6
title: >-
  Extent device: READ and WRITE data through shared memory instead of socket
  copies
status: To Do
assignee: []
created_date: '2026-10-15 08:19'
labels: []
dependencies: []
parent_task_id: TASK-26.18
priority: medium
type: enhancement
ordinal: 60000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The VMM copies a WRITE request data into a buffer and sends it over the Unix socket to fxdax-extentd, which copies it again; READ replies come back the same way. Guest memory is mapped in the VMM, and the daemon physical memory is a memfd, so the data could move without socket copies.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 READ and WRITE data no longer go through the socket
- [ ] #2 A GCE run shows WRITE request time per byte lower than before
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
