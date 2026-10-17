---
id: TASK-39.3
title: 'Live forks: fork a running VM''s store without closing it'
status: To Do
assignee: []
created_date: '2026-10-17 19:16'
labels: []
dependencies: []
parent_task_id: TASK-39
priority: high
type: feature
ordinal: 113000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Forking needs its source closed (fxdaxctl fork); the daemon's FORK request forks an open store but seals the guest's open extents under it. Snapshotting a running box needs a fork at a point the guest's filesystem agrees to: quiesce it (freeze, as fsfreeze does), commit, fork, thaw, while the VM keeps running.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A running VM's store is forked at a consistent point without stopping the VM, the guest's writes after it unaffected, shown by a guest test
- [ ] #2 The fork opens in another VM with the filesystem as of the fork point, shown by a test
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
