---
id: TASK-26.4
title: libkrun extent-store device backed by host memory and local disk
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-13 19:54'
updated_date: '2026-10-14 13:26'
labels: []
milestone: m-3
dependencies:
  - TASK-26.10
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
The host side of milestone 1 (doc-2, doc-4): a virtio device in libkrun serving the extent-store protocol over the local extent store (TASK-26.10), with extents' pages in fxdax's in-process kernel page cache keyed by extent ID and shared across VMs on the host, and the DAX window, userfaultfd faults, KVM pre-faulting and EFAULT retry that fxdax's virtio-fs device already uses. No object storage or forks yet.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The device implements doc-4's protocol over the local extent store, so a guest's filesystem survives restarting the VM
- [x] #2 Sealed extents map read-only into the DAX window from the shared page cache, and two windows in one VMM process mapping the same extent map the same host pages
- [ ] #3 Commits are durable in order: after a crash the store recovers to a prefix of the committed history, shown by a test
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. fxdax::ExtentStore: the store (extent_store) for a guest's own Fxfs, sync I/O on guest memory, doc-4 errnos, DAX window and MAP/UNMAP of sealed extents from a process-wide cache of their pages keyed by extent ID.
2. libkrun virtio/extent: device 0x4658, one queue, worker serving every doc-4 request, DAX window from the shared memory region, krun_add_extent_store, vmm wiring.
3. Prove with the guest driver (TASK-26.5): a guest test issuing every request, and later the guest's Fxfs surviving a VM restart (TASK-26.6).
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Done: fxdax 6f33b11 (ExtentStore, 3 tests), libkrun 9f1566f (device, worker tests; submodule commit local, not pushed). AC2 is met within a process (windows share an extent's cached pages); across VMs, which are separate processes, the in-process kernel's memory can't be shared: a design decision for the user (TODO). AC1 waits on TASK-26.5/26.6. AC3: the store's crash test (extent_store recovers_a_prefix_of_its_commits) and ExtentStore's reopen test; COMMIT replies once durable.

extent-guest (libkrun 14e17d7) exercises the device from a real guest kernel: every request, DAX MAP read in place, and the store holding the guest's commits after the VM exits.

AC2 narrowed (user, 2026-10-14): sharing within a VMM process is enough for now; it is met (fxdax 6f33b11: windows share an extent's cached pages). Across VMs, which libkrun runs as separate processes, would need shared-memory backing for the in-process kernel's pages or several VMs per process: deferred, not milestone 1.

AC1 met (2026-10-14): libkrun fxfs-guest and fxfs-guest-format (dbbd5ef, fxdax ca1113e): the guest's kernel Fxfs writes, unmounts and mounts again; after the VM exits a new process opens the store and the host's Fxfs finds the guest's files. Also the guest formats an empty store, which the host's Fxfs mounts.
<!-- SECTION:NOTES:END -->
