---
id: TASK-131.3
title: >-
  First reads of a shared base at ~130 MB/s with every base page in host memory:
  map once at boot, whole extents or 2 MiB per fault, base mappings kept ahead
  in reclaim, huge pages
status: To Do
assignee: []
created_date: '2026-10-29 21:13'
labels:
  - performance
dependencies: []
parent_task_id: TASK-131
priority: high
ordinal: 474000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Sandbox team round 8: VMs finding every base page in memory still read at ~130 MB/s. Options they list: map a fork's base into the window once at boot; map whole extents or 2 MiB per fault; keep base mappings ahead of writable ones when the window reclaims; huge pages. TASK-112.1 cut a first read of /usr from 2.31 s to 0.69 s with MAP_AHEAD (nested n2).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 First reads of a base in memory measured against each option, bare metal and nested, and the best landed
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
