---
id: TASK-26.4
title: libkrun extent-store device backed by host memory and local disk
status: To Do
assignee: []
created_date: '2026-10-13 19:54'
labels: []
milestone: m-3
dependencies:
  - TASK-26.2
documentation:
  - >-
    backlog/docs/doc-2 -
    Guest-owned-metadata-Fxfs-in-the-guest-kernel-over-a-virtio-extent-store.md
parent_task_id: TASK-26
type: feature
ordinal: 40000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The host side of milestone 1 (doc-2): a virtio device in libkrun serving the extent-store protocol, its extents held in fxdax's in-process kernel page cache (pager-backed, shared across VMs on the host) and persisted to a local file, with the DAX window, userfaultfd faults, KVM pre-faulting and the EFAULT retry that fxdax's virtio-fs device already uses. No object storage or forks yet.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The device implements the specified protocol, persisting extents and the root to local disk so a guest's filesystem survives restarting the VM
- [ ] #2 Committed extents map read-only into the DAX window from the shared page cache, and two VMs on one host mapping the same extent map the same host pages
- [ ] #3 Commits are durable in order: after a crash the store recovers to a prefix of the committed history, shown by a test
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
