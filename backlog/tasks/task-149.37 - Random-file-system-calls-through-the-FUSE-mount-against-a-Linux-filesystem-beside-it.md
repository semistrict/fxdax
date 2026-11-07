---
id: TASK-149.37
title: >-
  Random file system calls through the FUSE mount, against a Linux filesystem
  beside it
status: To Do
assignee: []
created_date: '2026-11-07 05:20'
labels:
  - testing
  - fuse
dependencies:
  - TASK-222
parent_task_id: TASK-149
priority: high
type: task
ordinal: 660000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The host's Node layer (crates/fxdax/src/filesystem.rs), which imports and now FUSE mounts (TASK-222) go through, plays Linux's VFS for Fxfs's POSIX layer, and has only example tests: nothing compares its semantics with Linux's. TASK-222 found it clearing a directory's setgid on chown, which Linux never does. With a store mounted through the kernel, the same random sequence of ordinary calls (open/creat with flags, mkdir, mknod, symlink, link, unlink, rmdir, rename and renameat2 flags, chmod, chown, utimensat, truncate, fallocate, read, write, pwrite past the end, xattrs, readdir, statx, fsync, setgid directories, umask) can run on the mount and on a tmpfs or ext4 directory beside it, as the guest's fsx differential runs against ext4 (TASK-149.12), comparing results, errnos and the trees after: Linux's own filesystem the oracle. Linux only; runs in CI's workspace part as the runner's user, through fusermount3, and as root on GCE.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A seeded, shrinking differential test runs random sequences of file system calls on a FUSE mount of a new store and on a directory of the host's own filesystem, and compares every call's result and errno, and the trees, attributes and contents after
- [ ] #2 Differences Linux allows (inode numbers, st_blocks, times' granularity, d_type of special files) are listed in one place, each with why
- [ ] #3 Each difference it finds is fixed or listed as known with a task, and the test is shown to find a mutant of Node::setattr and of the FUSE server, red
- [ ] #4 It runs in CI on Linux as the runner's user and on GCE as root
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
