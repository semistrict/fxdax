---
id: TASK-26.1
title: 'Spike: size porting Fxfs into the guest Linux kernel'
status: To Do
assignee: []
created_date: '2026-10-13 19:53'
labels: []
milestone: m-3
dependencies: []
documentation:
  - >-
    backlog/docs/doc-2 -
    Guest-owned-metadata-Fxfs-in-the-guest-kernel-over-a-virtio-extent-store.md
parent_task_id: TASK-26
type: spike
ordinal: 37000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Fxfs is async Rust written for Fuchsia, with dependencies on fuchsia-async, std, and Fuchsia libraries, and the design requires its code to run in the guest kernel (doc-2, requirement 5). Rust-for-Linux's support for filesystems (VFS bindings) and for async is limited. Before committing to milestone 1, find out what the port takes and whether it is feasible.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A report in the backlog covers: the kernel version to target and its Rust and VFS bindings; how Fxfs's async code runs in the kernel; allocation; which Fuchsia dependencies need stubbing or porting, with sizes; and an estimate with a go/no-go recommendation
- [ ] #2 A minimal Rust filesystem module builds against the chosen kernel and mounts in a libkrun guest
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
