---
id: TASK-72
title: Resident and pinned bytes per store in StatStore
status: To Do
assignee: []
created_date: '2026-10-21 19:54'
labels: []
dependencies: []
priority: low
ordinal: 181000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
/metrics and SetMemoryLimit work per tenant only. Report resident and pinned bytes per store in StatStore, so the host can attribute daemon memory to VMs.
<!-- SECTION:DESCRIPTION:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
