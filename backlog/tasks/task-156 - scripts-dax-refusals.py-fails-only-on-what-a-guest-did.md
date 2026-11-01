---
id: TASK-156
title: scripts/dax-refusals.py fails only on what a guest did
status: To Do
assignee: []
created_date: '2026-11-01 22:53'
labels: []
dependencies: []
priority: medium
type: task
ordinal: 557000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-150.16: refused stores flagged came from KVM async page fault kworkers, not vCPUs: all 2,327 in r422, all 28 in r394. The gate should join firecracker.log and fail on a refused store by a non-kworker thread or a repeated vCPU EFAULT run, and take counts from the daemon, whose log stops at 1,000 refusals (server.rs:1118).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 r394 and r422 logs pass, a store by a vCPU fails
- [ ] #2 Refusals past the log cap are counted
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
