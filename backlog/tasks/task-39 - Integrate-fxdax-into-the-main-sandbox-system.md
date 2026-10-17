---
id: TASK-39
title: Integrate fxdax into the main sandbox system
status: To Do
assignee: []
created_date: '2026-10-17 19:16'
labels: []
dependencies: []
priority: high
type: feature
ordinal: 110000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
What the main system (zygote restore, suspend-to-RAM, live migration, memory snapshots, tenant isolation, snapshot deletion, base images from Docker, our 6.1 kernel and Firecracker pin, placement fencing) needs of fxdax, from the integration review of 2026-10-17. Each subtask is one gap.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The subtasks are done
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
