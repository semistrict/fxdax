---
id: TASK-141
title: 'Upgrades: formats, version skew and rollback from the first release on'
status: To Do
assignee: []
created_date: '2026-10-31 20:30'
labels: []
dependencies: []
priority: high
ordinal: 487000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
doc-11: nothing lets a store outlive a format bump, the bucket's layout has no version, and a daemon upgrade ends every VM on the host. Carry out the plan.
<!-- SECTION:DESCRIPTION:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
