---
id: TASK-154
title: A tenant restores a snapshot on another host
status: To Do
assignee: []
created_date: '2026-11-01 22:53'
labels: []
dependencies: []
priority: high
type: feature
ordinal: 555000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Required: the system must handle cross-host restores. TASK-150.4 stopped a tenant naming a branch, as a compromised VM process could take another tenant by its ID, so a tenant restore where the point is not local now gets ENOENT. Without a format change, the host VM manager, which is trusted, could grant the tenant the branch through the control API (Grant, grants.rs), the daemon honouring OPEN_FORK_FROM only for a branch granted; an owner in the branch Record is the longer answer (the format bump).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A tenant restores on a host without the point, through its usual path
- [ ] #2 A tenant still can not take a branch not granted it: tenants_take_no_branch_by_naming_it holds
- [ ] #3 Shown end to end with two hosts or the simulator
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
