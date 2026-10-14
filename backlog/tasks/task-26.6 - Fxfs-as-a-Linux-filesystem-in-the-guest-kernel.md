---
id: TASK-26.6
title: Fxfs as a Linux filesystem in the guest kernel
status: To Do
assignee: []
created_date: '2026-10-13 19:54'
updated_date: '2026-10-14 13:26'
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

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Progress (2026-10-14, branch object-store): Fxfs builds for the guest (41a7c5e) against a sysroot of the guest std (0119d2a); locks sleep on kernel futexes and stacks are 64 KiB (8d0d1dc); mount -t fxfs <tag> over the extent device (184281c); writes, links, renames, setattr, fsync, statfs, and formatting empty stores (ca1113e). Tests: fxfs-guest, fxfs-guest-format. Left: page cache (mmap, exec), refusing a second mount of one store, symlink owners, RENAME_EXCHANGE, xattrs, pjdfstest, crash test, x86-64.
<!-- SECTION:NOTES:END -->
