---
id: TASK-159
title: The control socket follows the umask
status: To Do
assignee: []
created_date: '2026-11-02 02:53'
labels: []
dependencies: []
priority: medium
type: task
ordinal: 566000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
From TASK-155: the default socket is 0600 under a umask; the control socket still follows the process umask, so under umask 000 it is world-connectable, and it can do everything. It was left as a VM manager might rely on group access.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 It is 0600 or a chosen group, with no window, and a test shows it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
