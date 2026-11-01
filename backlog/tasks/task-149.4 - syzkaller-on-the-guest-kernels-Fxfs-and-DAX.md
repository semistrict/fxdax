---
id: TASK-149.4
title: syzkaller on the guest kernel's Fxfs and DAX
status: To Do
assignee: []
created_date: '2026-11-01 06:13'
labels: []
dependencies: []
parent_task_id: TASK-149
priority: medium
ordinal: 505000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Scripted operations find what their author thought of. syzkaller explores the system calls, including ioctls, flag combinations and racing calls, with coverage guidance. An unprivileged guest process must not be able to panic the guest or harm the host. TASK-77 fuzzes the protocol and fault path, not the VFS surface. Verify syzkaller's license before adding it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 syzkaller runs against a guest with Fxfs mounted, with KASAN and KCSAN
- [ ] #2 It has run on GCE, its crashes triaged and filed
- [ ] #3 Its syscall descriptions cover fxfs's ioctls and DAX mappings
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
