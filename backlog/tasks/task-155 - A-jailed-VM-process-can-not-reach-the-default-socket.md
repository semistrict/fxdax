---
id: TASK-155
title: A jailed VM process can not reach the default socket
status: To Do
assignee: []
created_date: '2026-11-01 22:53'
labels: []
dependencies: []
priority: medium
type: task
ordinal: 556000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A jail reaches only what is bind-mounted into its mount namespace, so the default socket is out of reach as long as the VM manager mounts only the tenant socket. Make that hold at the kernel, not by convention: the default socket owned by the daemon, mode 0600, so that a jail uid can not connect even if mounted.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The default socket refuses a process of another uid
- [ ] #2 A test shows it
- [ ] #3 doc says what the VM manager must do
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
