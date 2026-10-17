---
id: TASK-39.2
title: >-
  Tenant isolation: a VM process reaches only its own data, and the daemon's
  socket checks who asks
status: To Do
assignee: []
created_date: '2026-10-17 19:16'
labels: []
dependencies: []
parent_task_id: TASK-39
priority: high
type: feature
ordinal: 112000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Every Firecracker process maps the extent daemon's whole memory, which caches every VM's data (crates/fxdax/src/extentd/mod.rs and the README call a host's VMs 'one trust domain'): a guest that escapes into its VMM could read other tenants' data. And anything that reaches the daemon's socket can OPEN, create or FORK any store by its path.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A VM process maps only memory holding its own store's (and its lineage's) extents; a test shows another VM's cached pages are not reachable from it
- [ ] #2 The socket admits only callers it authorizes for a store (a capability handed over by the orchestrator, or peer credentials checked against a policy), and OPEN, create and FORK of paths outside it are refused, shown by tests
- [ ] #3 doc and README describe the isolation model that replaces 'one trust domain'
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
