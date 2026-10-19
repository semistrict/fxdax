---
id: TASK-50.3
title: 'The daemon''s memory: caps, reclaim instead of abort, per-tenant accounting'
status: To Do
assignee: []
created_date: '2026-10-19 20:10'
labels: []
dependencies: []
parent_task_id: TASK-50
priority: high
ordinal: 152000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
DAX moves each sandbox's file pages from its guest's RAM into extentd's memory, which no VM is charged for, so one tenant can starve a node. Today the daemon's memory is one startup argument (MEMORY_BYTES, 1 GiB by default), it aborts when that runs out, per-tenant arenas have no limits, and it reacts only to its own pressure, not the host's.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A total and a per-tenant cap, each changeable at runtime through the control API
- [ ] #2 At a cap the daemon evicts clean pages or refuses new allocations with an error the VM sees, and does not abort; a test fills a tenant past its cap
- [ ] #3 Per-tenant memory in /metrics, labelled by a hash of the tenant, not its id
- [ ] #4 A control call shrinks the daemon toward a target
- [ ] #5 How its memory is charged under cgroup v2 is documented
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
