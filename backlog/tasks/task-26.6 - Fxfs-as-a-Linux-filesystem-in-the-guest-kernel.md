---
id: TASK-26.6
title: Fxfs as a Linux filesystem in the guest kernel
status: To Do
assignee: []
created_date: '2026-10-13 19:54'
labels: []
milestone: m-3
dependencies:
  - TASK-26.1
  - TASK-26.3
  - TASK-26.5
documentation:
  - >-
    backlog/docs/doc-2 -
    Guest-owned-metadata-Fxfs-in-the-guest-kernel-over-a-virtio-extent-store.md
parent_task_id: TASK-26
type: feature
ordinal: 42000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The core of milestone 1 (doc-2): Fxfs's code, over the extent store, running as a filesystem in the guest kernel, so that metadata operations happen in guest memory and file data is read through DAX. Metadata is cached in guest memory and evicted like any cache; durability is POSIX with ordering preserved, a crash losing at most a suffix of history.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A libkrun guest mounts Fxfs from the extent-store device as its filesystem and passes the fxdax guest tests' filesystem checks and pjdfstest
- [ ] #2 Lookups, stats, creates, renames and unlinks issue no device requests once metadata is cached
- [ ] #3 After the VM is killed mid-workload, the filesystem mounts to a consistent prefix of its history, and fsync's ordering guarantees hold, shown by a test
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
