---
id: TASK-153.15
title: 'To try: a host that can ask a guest to seal'
status: To Do
assignee: []
created_date: '2026-11-02 01:59'
labels: []
dependencies: []
parent_task_id: TASK-153
priority: high
type: task
ordinal: 563000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-150.20: only a guest can seal, and the daemon can neither evict open pages nor ask, so one idle VM open pages can use the shared budget and another VM gets ENOMEM, when several run. Options: a device event asking a guest to seal, a per-tenant share of open pages, or evicting open pages to the store. Not run with several VMs at once.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A test with several VMs sharing a budget, one idle
- [ ] #2 Decided, from what it shows
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
