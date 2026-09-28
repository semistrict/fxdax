---
id: TASK-1
title: Survey libkrun virtio-fs server interface
status: To Do
assignee: []
created_date: '2026-09-28 02:41'
labels: []
milestone: m-0
dependencies: []
type: spike
ordinal: 1000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The virtio-fs server must plug into libkrun, whose virtio-fs device drives a FUSE server trait (inherited from crosvm/virtiofsd) and owns the DAX shared-memory region. Before writing the server we need to know which trait to implement, which FUSE ops the guest driver issues, how DAX setup/remove-mapping calls reach the server, and how requests are threaded (blocking vs async), since Fxfs is async and stream I/O on the kernel blocks.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A backlog doc records the server trait, the FUSE ops it dispatches, its threading model, and the DAX mapping hooks, with file references into libkrun
- [ ] #2 The doc lists which ops the Linux virtio-fs driver needs for a usable mount (mount, ls, cat, write, mkdir, rm, rename)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/kernel-cfg.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
