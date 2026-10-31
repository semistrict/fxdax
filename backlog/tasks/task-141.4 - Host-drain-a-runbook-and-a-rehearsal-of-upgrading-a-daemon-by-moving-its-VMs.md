---
id: TASK-141.4
title: 'Host drain: a runbook and a rehearsal of upgrading a daemon by moving its VMs'
status: To Do
assignee: []
created_date: '2026-10-31 20:30'
labels: []
dependencies: []
parent_task_id: TASK-141
ordinal: 491000
---

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A runbook covers drain, upgrade, check and rollback
- [ ] #2 scripts/migrate-gce.sh moves a VM from a host of N-1 to one of N and back, nightly
- [ ] #3 SIGTERM's stores are durable and closed before the daemon exits, by test
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
